//! Whole draws through the card: the draw commands, and the pixels
//! they leave in card memory against values worked out by hand.

use crate::toadforce::format::{Format, Rect, Surface};
use crate::toadforce::level1::encode;
use crate::toadforce::level1::states::*;
use crate::toadforce::level1::*;
use crate::toadforce::regs;
use crate::toadforce::{Live, ToadForce};
use crate::video::crt::CrtTiming;

const MEMORY: u32 = 32 << 20;
const COLOUR: u32 = 0x10_0000;
const DEPTH: u32 = 0x20_0000;
const TEXTURE: u32 = 0x30_0000;
const BUFFER: u32 = 0x40_0000;
const SIZE: u32 = 64;

/// Position with 1 / w, two colours and one coordinate set
/// (`D3DFVF_TLVERTEX`).
const TL: u32 = 0x1C4;

const BAD_VALUE: u32 = regs::Error::BadValue as u32;
const BAD_RANGE: u32 = regs::Error::BadRange as u32;
const BAD_STATE: u32 = regs::Error::BadState as u32;
const BAD_LENGTH: u32 = regs::Error::BadLength as u32;

/// A vertex of layout `TL` at depth 0 with w of 1.
fn vertex(x: f32, y: f32, diffuse: u32, u: f32, v: f32) -> [u32; 8] {
    vertex_z(x, y, 0.0, diffuse, u, v)
}

fn vertex_z(x: f32, y: f32, z: f32, diffuse: u32, u: f32, v: f32) -> [u32; 8] {
    [
        x.to_bits(),
        y.to_bits(),
        z.to_bits(),
        1.0f32.to_bits(),
        diffuse,
        0,
        u.to_bits(),
        v.to_bits(),
    ]
}

/// The clockwise triangle with its right angle at (`x`, `y`) and two
/// sides of `size`, in one colour.
fn corner(x: f32, y: f32, size: f32, z: f32, colour: u32) -> Vec<u32> {
    [
        vertex_z(x, y, z, colour, 0.0, 0.0),
        vertex_z(x + size, y, z, colour, 0.0, 0.0),
        vertex_z(x, y + size, z, colour, 0.0, 0.0),
    ]
    .concat()
}

/// The square from (0, 0) to (`size`, `size`) as a strip of four
/// vertices, with the texture once over it.
fn square(size: f32, colour: u32) -> Vec<u32> {
    [
        vertex(0.0, 0.0, colour, 0.0, 0.0),
        vertex(size, 0.0, colour, 1.0, 0.0),
        vertex(0.0, size, colour, 0.0, 1.0),
        vertex(size, size, colour, 1.0, 1.0),
    ]
    .concat()
}

struct Rig {
    card: ToadForce,
    memory: Vec<u8>,
    format: Format,
    depth: Option<Format>,
}

impl Rig {
    /// A card at level 1 with a 64x64 render target and a viewport of
    /// all of it.
    fn new(format: Format, depth: Option<Format>) -> Self {
        let mut rig = Rig {
            card: ToadForce::new(MEMORY, 1),
            memory: vec![0; MEMORY as usize],
            format,
            depth,
        };
        let live = Self::live();
        rig.card.write(regs::LEVEL, 1, &mut rig.memory, live);
        let target = Target {
            colour: rig.colour(),
            depth: depth.map(|format| Depth {
                offset: DEPTH,
                pitch: SIZE * format.bytes(),
                format,
            }),
        };
        let viewport = Viewport {
            x: 0,
            y: 0,
            width: SIZE,
            height: SIZE,
            min_z: 0.0,
            max_z: 1.0,
        };
        let error = rig.run(|w| {
            encode::render_target(w, Some(&target));
            encode::viewport(w, &viewport);
        });
        assert_eq!(error, 0);
        rig
    }

    fn live() -> Live {
        Live {
            ns: 0,
            vga: CrtTiming::VGA_400,
        }
    }

    fn colour(&self) -> Surface {
        Surface {
            offset: COLOUR,
            pitch: SIZE * self.format.bytes(),
            width: SIZE,
            height: SIZE,
            format: self.format,
        }
    }

    /// Run the commands `build` writes, and give the error they
    /// latched (0 for none), cleared for the next run.
    fn run(&mut self, build: impl FnOnce(&mut Vec<u32>)) -> u32 {
        let mut words = Vec::new();
        build(&mut words);
        self.card.submit(&words, &mut self.memory, Self::live());
        let error = self.card.read(regs::ERROR, Self::live());
        self.card
            .write(regs::ERROR, 0, &mut self.memory, Self::live());
        error
    }

    fn states(&mut self, pairs: &[(u32, u32)]) {
        assert_eq!(self.run(|w| encode::render_state(w, pairs)), 0);
    }

    fn stage(&mut self, stage: u32, pairs: &[(u32, u32)]) {
        assert_eq!(self.run(|w| encode::stage_state(w, stage, pairs)), 0);
    }

    /// Draw `vertices` of layout `TL` as `primitive`.
    fn draw(&mut self, primitive: u32, vertices: &[u32]) -> u32 {
        self.run(|w| encode::draw(w, primitive, TL, vertices.len() as u32 / 8, vertices))
    }

    fn clear(&mut self, flags: u32, colour: u32, z: f32, stencil: u32) {
        assert_eq!(
            self.run(|w| encode::clear(w, flags, colour, z, stencil, &[])),
            0
        );
    }

    fn word(&self, at: usize, bytes: u32) -> u32 {
        let mut word = [0; 4];
        word[..bytes as usize].copy_from_slice(&self.memory[at..at + bytes as usize]);
        u32::from_le_bytes(word)
    }

    /// The pixel of the render target at (`x`, `y`).
    fn px(&self, x: u32, y: u32) -> u32 {
        self.word(self.colour().at(x, y), self.format.bytes())
    }

    /// The pixel of the depth buffer there.
    fn depth(&self, x: u32, y: u32) -> u32 {
        let bytes = self.depth.unwrap().bytes();
        self.word((DEPTH + (y * SIZE + x) * bytes) as usize, bytes)
    }

    /// The pixels of the target that aren't `background`.
    fn drawn(&self, background: u32) -> Vec<(u32, u32)> {
        (0..SIZE)
            .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
            .filter(|&(x, y)| self.px(x, y) != background)
            .collect()
    }

    /// Define a texture of `texels` rows at `TEXTURE` as handle 1, and
    /// bind it to stage 0, which takes its colour and alpha.
    fn texture(&mut self, format: Format, texels: &[&[u32]], key: Option<u32>) {
        let (w, h) = (texels[0].len() as u32, texels.len() as u32);
        let bytes = format.bytes();
        for (y, row) in texels.iter().enumerate() {
            for (x, texel) in row.iter().enumerate() {
                let at = (TEXTURE + (y as u32 * w + x as u32) * bytes) as usize;
                self.memory[at..at + bytes as usize]
                    .copy_from_slice(&texel.to_le_bytes()[..bytes as usize]);
            }
        }
        let texture = Texture {
            format,
            width: w,
            height: h,
            levels: 1,
            key,
            images: vec![(TEXTURE, w * bytes)],
            ..Texture::default()
        };
        let error = self.run(|w| {
            encode::define_texture(w, 1, &texture);
            encode::set_texture(w, 0, 1);
        });
        assert_eq!(error, 0);
        self.stage(
            0,
            &[
                (TSS_COLOROP, OP_SELECT_ARG1),
                (TSS_COLORARG1, ARG_TEXTURE),
                (TSS_ALPHAOP, OP_SELECT_ARG1),
                (TSS_ALPHAARG1, ARG_TEXTURE),
            ],
        );
    }
}

#[test]
fn a_triangle_fills_the_pixels_whose_centres_it_holds() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    assert_eq!(rig.draw(4, &corner(2.0, 3.0, 10.0, 0.0, 0xFF33_6699)), 0);
    let drawn = rig.drawn(0);
    // 10 pixels in its top row, one fewer in each row below.
    assert_eq!(drawn.len(), 55);
    assert!(drawn.iter().all(|&(x, y)| rig.px(x, y) == 0x33_6699));
    assert_eq!(drawn[0], (2, 3));
    assert_eq!(drawn[9], (11, 3));
    assert_eq!(drawn[54], (2, 12));
    // Counterclockwise, it is culled after a reset.
    let mut back = corner(20.0, 20.0, 10.0, 0.0, 0xFFFF_FFFF);
    back.swap(8, 16);
    back.swap(9, 17);
    assert_eq!(rig.draw(4, &back), 0);
    assert_eq!(rig.drawn(0).len(), 55);
    rig.states(&[(RS_CULLMODE, 1)]);
    assert_eq!(rig.draw(4, &back), 0);
    assert_eq!(rig.drawn(0).len(), 110);
    let counts = rig.card.describe(Rig::live());
    assert_eq!(counts["commands"]["draws"], 3);
    assert_eq!(counts["commands"]["primitives"], 2);
}

#[test]
fn colours_are_interpolated_between_the_vertices() {
    let mut rig = Rig::new(Format::A8R8G8B8, None);
    let vertices = [
        vertex(0.0, 0.0, 0xFF00_0000, 0.0, 0.0),
        vertex(64.0, 0.0, 0xFF80_0040, 0.0, 0.0),
        vertex(0.0, 64.0, 0x7F00_0000, 0.0, 0.0),
    ]
    .concat();
    assert_eq!(rig.draw(4, &vertices), 0);
    // Red goes up 2 a pixel to the right and blue 1; alpha goes down
    // 2 a pixel down.
    assert_eq!(rig.px(0, 0), 0xFF00_0000);
    assert_eq!(rig.px(16, 0), 0xFF20_0010);
    assert_eq!(rig.px(16, 8), 0xEF20_0010);
    assert_eq!(rig.px(1, 62), 0x8302_0001);
    // Flat, the first vertex's colour, or the last's.
    rig.states(&[(RS_SHADEMODE, 1)]);
    assert_eq!(rig.draw(4, &vertices), 0);
    assert_eq!(rig.px(16, 8), 0xFF00_0000);
    rig.states(&[(RS_FLAT_VERTEX, 1)]);
    assert_eq!(rig.draw(4, &vertices), 0);
    assert_eq!(rig.px(16, 8), 0x7F00_0000);
}

#[test]
fn a_texture_drawn_texel_for_pixel_is_copied() {
    let texels: [&[u32]; 4] = [
        &[0xFF10_2030, 0x8011_2131, 0x0012_2232, 0x4013_2333],
        &[0xFF40_5060, 0xFF41_5161, 0xFF42_5262, 0xFF43_5363],
        &[0xFF70_8090, 0xFF71_8191, 0xFF72_8292, 0xFF73_8393],
        &[0xFFA0_B0C0, 0xFFA1_B1C1, 0xFFA2_B2C2, 0x01A3_B3C3],
    ];
    let mut rig = Rig::new(Format::A8R8G8B8, None);
    rig.texture(Format::A8R8G8B8, &texels, None);
    assert_eq!(rig.draw(5, &square(4.0, 0xFFFF_FFFF)), 0);
    for (y, row) in texels.iter().enumerate() {
        for (x, texel) in row.iter().enumerate() {
            assert_eq!(rig.px(x as u32, y as u32), *texel, "({x}, {y})");
        }
    }
    assert_eq!(rig.drawn(0).len(), 16);

    // Twice the size, each texel is four pixels, and modulated with
    // half grey it is halved.
    rig.stage(
        0,
        &[(TSS_COLOROP, OP_MODULATE), (TSS_COLORARG2, ARG_DIFFUSE)],
    );
    assert_eq!(rig.draw(5, &square(8.0, 0xFF80_8080)), 0);
    // 80h of 40h, 50h and 60h, rounded.
    assert_eq!(rig.px(1, 3), 0xFF20_2830);
    assert_eq!(rig.px(0, 2), 0xFF20_2830);
    assert_eq!(rig.px(2, 2), 0xFF21_2931);

    // A 5-6-5 texture into a 5-6-5 target comes out as it went in.
    let texels: [&[u32]; 2] = [&[0x1234, 0xFFFF], &[0x0821, 0xF81F]];
    let mut rig = Rig::new(Format::R5G6B5, None);
    rig.texture(Format::R5G6B5, &texels, None);
    assert_eq!(rig.draw(5, &square(2.0, 0xFFFF_FFFF)), 0);
    assert_eq!(
        [rig.px(0, 0), rig.px(1, 0), rig.px(0, 1), rig.px(1, 1)],
        [0x1234, 0xFFFF, 0x0821, 0xF81F]
    );
}

#[test]
fn the_colour_key_leaves_its_texels_out() {
    let texels: [&[u32]; 2] = [&[0xF81F, 0x07E0], &[0x07E0, 0xF81F]];
    let mut rig = Rig::new(Format::R5G6B5, None);
    rig.texture(Format::R5G6B5, &texels, Some(0xF81F));
    // The key counts only with its state on.
    assert_eq!(rig.draw(5, &square(2.0, 0xFFFF_FFFF)), 0);
    assert_eq!(rig.drawn(0), [(0, 0), (1, 0), (0, 1), (1, 1)]);
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    rig.states(&[(RS_COLORKEYENABLE, 1)]);
    assert_eq!(rig.draw(5, &square(2.0, 0xFFFF_FFFF)), 0);
    assert_eq!(rig.drawn(0), [(1, 0), (0, 1)]);
}

#[test]
fn the_depth_test_keeps_the_nearer_fragment() {
    let mut rig = Rig::new(Format::X8R8G8B8, Some(Format::D16));
    rig.clear(CLEAR_DEPTH, 0, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.5, 0xFF00_00AA)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0xAA, 0x8000));
    assert_eq!(rig.depth(8, 0), 0xFFFF);
    // Behind it, nothing; in front of it, the new one. Equal depth
    // passes the test of a reset card.
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.75, 0xFF00_00BB)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0xAA, 0x8000));
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.5, 0xFF00_00CC)), 0);
    assert_eq!(rig.px(1, 1), 0xCC);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 4.0, 0.25, 0xFF00_00DD)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0xDD, 0x4000));
    assert_eq!((rig.px(5, 1), rig.depth(5, 1)), (0xCC, 0x8000));
    // Without depth writes the test still runs.
    rig.states(&[(RS_ZWRITEENABLE, 0)]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.125, 0xFF00_00EE)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0xEE, 0x4000));
    // A bias brings a fragment nearer by that many steps.
    rig.states(&[(RS_ZWRITEENABLE, 1), (RS_ZFUNC, 2), (RS_ZBIAS, 3)]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 4.0, 0.25, 0xFF00_0011)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0x11, 0x3FFD));
}

#[test]
fn a_fragment_the_alpha_test_drops_writes_no_depth() {
    let mut rig = Rig::new(Format::X8R8G8B8, Some(Format::D24X8));
    rig.clear(CLEAR_DEPTH, 0, 1.0, 0);
    rig.states(&[
        (RS_ZENABLE, 1),
        (RS_ALPHATESTENABLE, 1),
        (RS_ALPHAFUNC, 5),
        (RS_ALPHAREF, 0x80),
    ]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.5, 0x8000_00AA)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0, 0xFFFF_FF00));
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.5, 0x8100_00AA)), 0);
    assert_eq!((rig.px(1, 1), rig.depth(1, 1)), (0xAA, 0x8000_0000));
}

#[test]
fn the_stencil_marks_pixels_and_limits_later_draws_to_them() {
    let mut rig = Rig::new(Format::X8R8G8B8, Some(Format::D24S8));
    rig.clear(CLEAR_DEPTH | CLEAR_STENCIL, 0, 1.0, 0);
    // Write 55h where a triangle is, and no colour.
    rig.states(&[
        (RS_STENCILENABLE, 1),
        (RS_STENCILPASS, 3),
        (RS_STENCILREF, 0x55),
        (RS_COLORWRITEENABLE, 0),
    ]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 4.0, 0.0, 0xFFFF_FFFF)), 0);
    assert!(rig.drawn(0).is_empty());
    assert_eq!(rig.depth(1, 1), 0xFFFF_FF55);
    assert_eq!(rig.depth(4, 0), 0xFFFF_FF00);
    // Draw where the stencil is 55h, and count it up there.
    rig.states(&[
        (RS_STENCILFUNC, 3),
        (RS_STENCILPASS, 7),
        (RS_STENCILFAIL, 2),
        (RS_STENCILWRITEMASK, 0x0F),
        (RS_COLORWRITEENABLE, 0xF),
    ]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.0, 0xFF00_00AA)), 0);
    assert_eq!(rig.drawn(0).len(), 10);
    // Passed: 55h + 1 in the low four bits. Failed: 0 in them.
    assert_eq!(rig.depth(1, 1), 0xFFFF_FF56);
    assert_eq!(rig.depth(5, 1), 0xFFFF_FF00);
}

#[test]
fn blending_weighs_the_fragment_against_the_pixel_there() {
    let mut rig = Rig::new(Format::A8R8G8B8, None);
    rig.clear(CLEAR_COLOUR, 0xFF20_4060, 0.0, 0);
    rig.states(&[
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 5),
        (RS_DESTBLEND, 6),
    ]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 4.0, 0.0, 0x80FF_FFFF)), 0);
    // 128 of 255 of white over the pixel, each channel rounded.
    assert_eq!(rig.px(1, 1), 0xBF90_A0B0);
    assert_eq!(rig.px(4, 0), 0xFF20_4060);
    // Additive, with only red and alpha written.
    rig.states(&[
        (RS_SRCBLEND, 2),
        (RS_DESTBLEND, 2),
        (RS_COLORWRITEENABLE, 9),
    ]);
    assert_eq!(rig.draw(4, &corner(8.0, 0.0, 4.0, 0.0, 0x10F0_F0F0)), 0);
    assert_eq!(rig.px(9, 1), 0xFFFF_4060);
}

#[test]
fn specular_and_fog_follow_the_stages() {
    let mut rig = Rig::new(Format::X8R8G8B8, Some(Format::D16));
    let mut vertices = corner(0.0, 0.0, 8.0, 0.5, 0xFF40_4040);
    for v in 0..3 {
        // Specular colour, with the vertex's fog factor in its alpha.
        vertices[v * 8 + 5] = 0x4010_2030;
    }
    rig.states(&[(RS_SPECULARENABLE, 1)]);
    assert_eq!(rig.draw(4, &vertices), 0);
    assert_eq!(rig.px(1, 1), 0x50_6070);
    // Vertex fog: 40h of 255 of the colour, the rest fog.
    rig.states(&[(RS_FOGENABLE, 1), (RS_FOGCOLOR, 0x00FF_0000)]);
    assert_eq!(rig.draw(4, &vertices), 0);
    assert_eq!(rig.px(1, 1), 0xD3_181C);
    // Linear table fog by depth: half way from start to end.
    rig.states(&[
        (RS_FOGTABLEMODE, 3),
        (RS_FOGSTART, 0.0f32.to_bits()),
        (RS_FOGEND, 1.0f32.to_bits()),
        (RS_FOGCOLOR, 0),
        (RS_SPECULARENABLE, 0),
    ]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.5, 0xFFFF_FFFF)), 0);
    assert_eq!(rig.px(1, 1), 0x80_8080);
}

#[test]
fn points_and_lines_cover_their_pixels() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    let white = 0xFFFF_FFFF;
    // A point is the pixel its centre is in, or a square around it.
    assert_eq!(rig.draw(1, &vertex(5.0, 6.0, white, 0.0, 0.0)), 0);
    assert_eq!(rig.drawn(0), [(5, 6)]);
    rig.states(&[(RS_POINTSIZE, 3.0f32.to_bits())]);
    assert_eq!(rig.draw(1, &vertex(20.0, 20.0, white, 0.0, 0.0)), 0);
    let drawn = rig.drawn(0);
    assert_eq!(drawn.len(), 10);
    assert_eq!((drawn[1], drawn[9]), ((19, 19), (21, 21)));

    // A line has a pixel in each column, or each row, from its start
    // to its end, the end's with `LASTPIXEL` only.
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    let line = |x0: f32, y0: f32, x1: f32, y1: f32| {
        [
            vertex(x0, y0, white, 0.0, 0.0),
            vertex(x1, y1, white, 0.0, 0.0),
        ]
        .concat()
    };
    assert_eq!(rig.draw(2, &line(1.0, 1.0, 5.0, 1.0)), 0);
    assert_eq!(rig.drawn(0), [(1, 1), (2, 1), (3, 1), (4, 1), (5, 1)]);
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    rig.states(&[(RS_LASTPIXEL, 0)]);
    assert_eq!(rig.draw(2, &line(5.0, 1.0, 1.0, 3.0)), 0);
    assert_eq!(rig.drawn(0), [(5, 1), (3, 2), (4, 2), (2, 3)]);
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    assert_eq!(rig.draw(2, &line(2.0, 6.0, 3.0, 2.0)), 0);
    assert_eq!(rig.drawn(0), [(3, 3), (3, 4), (2, 5), (2, 6)]);

    // A strip of lines, and a triangle as its outline.
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    let strip = [line(0.0, 0.0, 3.0, 0.0), line(3.0, 3.0, 0.0, 3.0)].concat();
    assert_eq!(rig.draw(3, &strip), 0);
    assert_eq!(rig.drawn(0).len(), 9);
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    rig.states(&[(RS_FILLMODE, 2)]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 4.0, 0.0, white)), 0);
    assert_eq!(rig.drawn(0).len(), 12);
    assert_eq!(rig.px(1, 1), 0);

    // The points of a triangle above the target reach into its first
    // row: only the triangle's winding decides whether they are drawn.
    rig.clear(CLEAR_COLOUR, 0, 0.0, 0);
    rig.states(&[(RS_FILLMODE, 1), (RS_POINTSIZE, 4.0f32.to_bits())]);
    assert_eq!(rig.draw(4, &corner(4.0, -3.0, 2.0, 0.0, white)), 0);
    assert_eq!(rig.drawn(0), [(2, 0), (3, 0), (4, 0), (5, 0)]);
}

#[test]
fn a_buffer_a_draw_paints_over_is_read_after_it() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    // A white triangle's vertices in row 40 of the target, which the
    // first draw of the run paints over with positions of no area.
    let at = COLOUR as usize + 40 * (SIZE * 4) as usize;
    for (i, word) in corner(0.0, 0.0, 8.0, 0.0, 0xFFFF_FFFF).iter().enumerate() {
        rig.memory[at + 4 * i..][..4].copy_from_slice(&word.to_le_bytes());
    }
    let error = rig.run(|w| {
        encode::draw(w, 5, TL, 4, &square(64.0, 0xFF12_3456));
        encode::draw_buffer(w, 4, TL, 32, at as u32, 3);
    });
    assert_eq!(error, 0);
    assert_eq!(rig.px(1, 1), 0x12_3456);
}

#[test]
fn the_four_draw_commands_draw_the_same() {
    let vertices = square(8.0, 0xFF12_3456);
    let mut rigs: Vec<Rig> = (0..5).map(|_| Rig::new(Format::X8R8G8B8, None)).collect();
    for rig in &mut rigs {
        // The vertices 12 bytes apart from the last's end, and the
        // indices of two triangles after them.
        for (v, vertex) in vertices.chunks(8).enumerate() {
            for (i, word) in vertex.iter().enumerate() {
                let at = BUFFER as usize + v * 44 + i * 4;
                rig.memory[at..at + 4].copy_from_slice(&word.to_le_bytes());
            }
        }
        for (i, index) in [0u16, 1, 2, 2, 1, 3].iter().enumerate() {
            let at = BUFFER as usize + 0x1000 + i * 2;
            rig.memory[at..at + 2].copy_from_slice(&index.to_le_bytes());
        }
    }
    let index = [0u16, 1, 2, 2, 1, 3];
    let errors = [
        rigs[0].draw(5, &vertices),
        rigs[1].run(|w| encode::draw_indexed(w, 4, TL, 4, &vertices, &index)),
        rigs[2].run(|w| encode::draw_buffer(w, 5, TL, 44, BUFFER, 4)),
        rigs[3].run(|w| {
            encode::draw_buffer_indexed(w, 4, TL, 44, BUFFER, 4, Ok((BUFFER + 0x1000, 6)))
        }),
        rigs[4].run(|w| encode::draw_buffer_indexed(w, 4, TL, 44, BUFFER, 4, Err(&index))),
    ];
    assert_eq!(errors, [0; 5]);
    assert_eq!(rigs[0].drawn(0).len(), 64);
    for rig in &rigs[1..] {
        assert!(rig.memory == rigs[0].memory);
    }
    // A fan from the square's first corner.
    let mut fan = Rig::new(Format::X8R8G8B8, None);
    let order = [0u16, 1, 3, 2];
    assert_eq!(
        fan.run(|w| encode::draw_indexed(w, 6, TL, 4, &vertices, &order)),
        0
    );
    assert_eq!(fan.drawn(0), rigs[0].drawn(0));
}

#[test]
fn a_draw_the_card_cant_take_is_rejected_whole() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    let vertices = corner(0.0, 0.0, 8.0, 0.0, 0xFFFF_FFFF);
    let header = |len: usize| crate::toadforce::command::header(DRAW, len);
    let with = |primitive: u32, fvf: u32, count: u32| {
        let mut words = vec![header(4 + vertices.len()), primitive, fvf, count];
        words.extend(&vertices);
        words
    };
    let run = |rig: &mut Rig, words: Vec<u32>| rig.run(|w| w.extend(words));
    assert_eq!(run(&mut rig, with(0, TL, 3)), BAD_VALUE);
    assert_eq!(run(&mut rig, with(7, TL, 3)), BAD_VALUE);
    // A layout that isn't one, and one the card doesn't transform yet.
    assert_eq!(run(&mut rig, with(4, 0x1C1, 3)), BAD_VALUE);
    assert_eq!(run(&mut rig, with(4, 0x142, 3)), BAD_VALUE);
    assert_eq!(run(&mut rig, with(4, TL, 0x1_0001)), BAD_VALUE);
    // The count and the layout say how long the command is.
    assert_eq!(run(&mut rig, with(4, TL, 2)), BAD_LENGTH);
    assert_eq!(run(&mut rig, with(4, TL, 4)), BAD_LENGTH);
    assert_eq!(run(&mut rig, with(4, 0x044, 3)), BAD_LENGTH);
    // Buffers inside card memory, strides that hold a vertex.
    let buffer = |stride: u32, offset: u32, count: u32| {
        let mut words = Vec::new();
        encode::draw_buffer(&mut words, 4, TL, stride, offset, count);
        words
    };
    assert_eq!(run(&mut rig, buffer(31, BUFFER, 3)), BAD_VALUE);
    assert_eq!(run(&mut rig, buffer(1028, BUFFER, 3)), BAD_VALUE);
    assert_eq!(run(&mut rig, buffer(32, MEMORY - 95, 3)), BAD_RANGE);
    assert_eq!(run(&mut rig, buffer(32, MEMORY - 96, 3)), 0);
    assert_eq!(run(&mut rig, buffer(32, MEMORY, 0)), 0);
    let indexed = |index: Result<(u32, u32), &[u16]>| {
        let mut words = Vec::new();
        encode::draw_buffer_indexed(&mut words, 4, TL, 32, BUFFER, 3, index);
        words
    };
    assert_eq!(run(&mut rig, indexed(Ok((MEMORY - 5, 3)))), BAD_RANGE);
    // An index past the vertices, in the command and in memory.
    assert_eq!(run(&mut rig, indexed(Err(&[0, 1, 3]))), BAD_RANGE);
    rig.memory[BUFFER as usize + 0x1000] = 3;
    assert_eq!(run(&mut rig, indexed(Ok((BUFFER + 0x1000, 3)))), BAD_RANGE);
    assert!(rig.drawn(0).is_empty());

    // The state comes last: no target, and too many textures.
    assert_eq!(rig.run(|w| encode::render_target(w, None)), 0);
    assert_eq!(rig.draw(4, &vertices), BAD_STATE);
}

#[test]
fn draws_and_other_commands_keep_their_order() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    let surface = rig.colour();
    let all = Rect {
        left: 0,
        top: 0,
        width: SIZE,
        height: SIZE,
    };
    let tri = corner(0.0, 0.0, 8.0, 0.0, 0xFFAA_AAAA);
    // A draw, a fill over it, and a draw over that, in one run.
    let error = rig.run(|w| {
        encode::draw(w, 4, TL, 3, &tri);
        crate::toadforce::command::encode::fill(w, &surface, &all, 0x11_1111);
        encode::draw(w, 4, TL, 3, &corner(0.0, 0.0, 4.0, 0.0, 0xFFBB_BBBB));
        encode::clear(
            w,
            CLEAR_COLOUR,
            0x22_2222,
            0.0,
            0,
            &[Rect { left: 0, ..all }],
        );
        encode::draw(w, 4, TL, 3, &corner(0.0, 0.0, 2.0, 0.0, 0xFFCC_CCCC));
        // A state set after a draw doesn't reach back to it.
        encode::render_state(w, &[(RS_COLORWRITEENABLE, 0)]);
        encode::draw(w, 4, TL, 3, &tri);
    });
    assert_eq!(error, 0);
    assert_eq!(rig.px(0, 0), 0xCC_CCCC);
    assert_eq!(rig.px(3, 0), 0x22_2222);
    assert_eq!(rig.px(40, 40), 0x22_2222);
}

/// A scene with what differs from pixel to pixel: many triangles over
/// each other with a filtered texture, depth, blending and dither.
fn scene(workers: usize, format: Format) -> Vec<u8> {
    let mut rig = Rig::new(format, Some(Format::D24S8));
    rig.card.fix_workers(workers);
    let texels: Vec<Vec<u32>> = (0..8u32)
        .map(|y| {
            (0..8u32)
                .map(|x| 0x8000_0000 | (x * 36) << 16 | (y * 36) << 8 | ((x ^ y) * 36))
                .collect()
        })
        .collect();
    let rows: Vec<&[u32]> = texels.iter().map(|row| &row[..]).collect();
    rig.texture(Format::A8R8G8B8, &rows, None);
    rig.stage(
        0,
        &[
            (TSS_COLOROP, OP_MODULATE),
            (TSS_COLORARG2, ARG_DIFFUSE),
            (TSS_MAGFILTER, 2),
            (TSS_MINFILTER, 2),
        ],
    );
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF10_2030, 1.0, 0);
    rig.states(&[
        (RS_ZENABLE, 1),
        (RS_CULLMODE, 1),
        (RS_DITHERENABLE, 1),
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 5),
        (RS_DESTBLEND, 6),
    ]);
    let mut vertices = Vec::new();
    let mut seed = 12345u32;
    let mut next = |range: f32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1 << 24) as f32 * range
    };
    for _ in 0..60 {
        for _ in 0..3 {
            let (x, y) = (next(80.0) - 8.0, next(80.0) - 8.0);
            let mut v = vertex_z(x, y, next(1.0), 0xFFFF_FFFF, next(3.0), next(3.0));
            v[3] = (0.25 + next(1.0)).to_bits();
            v[4] = next(4e9) as u32 | 0x4040_4040;
            vertices.extend(v);
        }
    }
    assert_eq!(rig.draw(4, &vertices), 0);
    rig.memory
}

#[test]
fn the_picture_is_the_same_for_any_number_of_threads() {
    for format in [Format::R5G6B5, Format::A8R8G8B8] {
        let alone = scene(0, format);
        assert!(alone[COLOUR as usize..][..4096].iter().any(|&b| b != 0));
        for workers in [1, 3, 6] {
            assert!(scene(workers, format) == alone, "{workers} workers");
        }
    }
}

#[test]
fn a_loaded_card_draws_as_the_saved_one() {
    use crate::savestate::{Reader, State, Writer};
    let mut rig = Rig::new(Format::A8R8G8B8, Some(Format::D16));
    let texels: [&[u32]; 2] = [&[0xFF10_2030, 0x80FF_0000], &[0x4000_FF00, 0xFFFF_FFFF]];
    rig.texture(Format::A8R8G8B8, &texels, None);
    rig.states(&[
        (RS_ZENABLE, 1),
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 5),
        (RS_DESTBLEND, 6),
    ]);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF40_4040, 1.0, 0);
    // The first draw keeps the lowered state beside the card's.
    assert_eq!(rig.draw(5, &square(8.0, 0xFFFF_FFFF)), 0);
    let mut w = Writer::new();
    rig.card.save(&mut w);
    let mut loaded = Rig {
        card: ToadForce::new(MEMORY, 1),
        memory: rig.memory.clone(),
        format: rig.format,
        depth: rig.depth,
    };
    loaded.card.load(&mut Reader::new(&w.buf)).unwrap();
    for rig in [&mut rig, &mut loaded] {
        assert_eq!(rig.draw(5, &square(6.0, 0x80C0_C0C0)), 0);
    }
    assert!(rig.px(2, 2) != 0xFF40_4040);
    assert!(loaded.memory == rig.memory);
}
